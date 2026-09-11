use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use thiserror::Error;

use crate::catalog::ActionId;
use crate::runtime::ResolvedCommand;

const EVENT_BUFFER: usize = 128;
const OUTPUT_CHUNK: usize = 8 * 1024;
const OUTPUT_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecutionStatus {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionResult {
    pub action_id: ActionId,
    pub status: ExecutionStatus,
    pub exit_code: Option<u32>,
    pub signal: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeEvent {
    Started { action_id: ActionId },
    Output(Vec<u8>),
    Exited(ExecutionResult),
    Failed(String),
}

#[derive(Debug, Error)]
pub enum StartError {
    #[error("could not allocate a pseudo-terminal: {0}")]
    Allocate(String),
    #[error("could not open the pseudo-terminal output stream: {0}")]
    OpenReader(String),
    #[error("could not open the pseudo-terminal input stream: {0}")]
    OpenWriter(String),
    #[error("could not launch action `{action}`: {message}")]
    Spawn { action: ActionId, message: String },
    #[error("could not synchronize action `{action}` before launch: {message}")]
    Synchronize { action: ActionId, message: String },
    #[error("could not start the action output monitor: {0}")]
    Monitor(std::io::Error),
}

pub struct RunningSession {
    action_id: ActionId,
    master: Box<dyn MasterPty + Send>,
    writer: Mutex<Option<Box<dyn Write + Send>>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    events: Option<Receiver<RuntimeEvent>>,
    monitor: Option<JoinHandle<()>>,
    cancellation_requested: Arc<AtomicBool>,
    exited: bool,
}

impl RunningSession {
    pub fn start(command: ResolvedCommand, rows: u16, cols: u16) -> Result<Self, StartError> {
        let action_id = command.action_id.clone();
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| StartError::Allocate(error.to_string()))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| StartError::OpenReader(error.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| StartError::OpenWriter(error.to_string()))?;

        let (sender, events) = sync_channel(EVENT_BUFFER);
        let _ = sender.send(RuntimeEvent::Started {
            action_id: action_id.clone(),
        });
        // The child stops itself before exec so the output reader is active
        // before even a single-instruction action can write and exit. This is
        // required on FreeBSD, where unread PTY bytes may be discarded when a
        // short-lived child closes the slave.
        let mut builder = CommandBuilder::new("/bin/sh");
        builder.args(["-c", "kill -STOP $$; exec \"$@\"", "crank-action"]);
        builder.arg(&command.program);
        builder.args(&command.args);
        builder.cwd(&command.working_directory);
        let mut child = pair
            .slave
            .spawn_command(builder)
            .map_err(|error| StartError::Spawn {
                action: action_id.clone(),
                message: error.to_string(),
            })?;
        let child_pid = match child.process_id() {
            Some(pid) => pid,
            None => {
                let _ = child.kill();
                return Err(StartError::Synchronize {
                    action: action_id.clone(),
                    message: "the PTY backend did not expose the child process ID".to_owned(),
                });
            }
        };
        if let Err(error) = wait_until_stopped(child_pid) {
            let _ = child.kill();
            return Err(StartError::Synchronize {
                action: action_id.clone(),
                message: error.to_string(),
            });
        }
        let output_fd = pair
            .master
            .as_raw_fd()
            .expect("the native Unix PTY exposes a file descriptor");
        let child_exited = Arc::new(AtomicBool::new(false));
        let output_child_exited = Arc::clone(&child_exited);
        let output_sender = sender.clone();
        let output = thread::Builder::new()
            .name("crank-action-output".to_owned())
            .spawn(move || copy_output(reader, output_fd, output_child_exited, output_sender))
            .map_err(|error| {
                let _ = child.kill();
                StartError::Monitor(error)
            })?;
        if unsafe { libc::kill(child_pid as libc::pid_t, libc::SIGCONT) } != 0 {
            let error = std::io::Error::last_os_error();
            let _ = child.kill();
            child_exited.store(true, Ordering::Release);
            drop(pair.slave);
            let _ = output.join();
            return Err(StartError::Synchronize {
                action: action_id.clone(),
                message: error.to_string(),
            });
        }
        let killer = child.clone_killer();
        let cancellation_requested = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::clone(&cancellation_requested);
        let slave = pair.slave;
        let monitor = thread::spawn(move || {
            let status = child.wait();
            child_exited.store(true, Ordering::Release);
            let _ = output.join();
            drop(slave);
            match status {
                Ok(status) => {
                    let result = ExecutionResult {
                        action_id: command.action_id.clone(),
                        status: if cancelled.load(Ordering::SeqCst) {
                            ExecutionStatus::Cancelled
                        } else if status.success() {
                            ExecutionStatus::Succeeded
                        } else {
                            ExecutionStatus::Failed
                        },
                        exit_code: status.signal().is_none().then(|| status.exit_code()),
                        signal: status.signal().map(str::to_owned),
                    };
                    let _ = sender.send(RuntimeEvent::Exited(result));
                }
                Err(error) => {
                    let _ = sender.send(RuntimeEvent::Failed(format!(
                        "could not wait for action `{}`: {error}",
                        command.action_id
                    )));
                }
            }
            // Keep the extracted catalog alive until the monitored child has exited.
            drop(command);
        });

        Ok(Self {
            action_id,
            master: pair.master,
            writer: Mutex::new(Some(writer)),
            killer: Mutex::new(killer),
            events: Some(events),
            monitor: Some(monitor),
            cancellation_requested,
            exited: false,
        })
    }

    pub fn action_id(&self) -> &ActionId {
        &self.action_id
    }

    pub fn write_input(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut writer = self.writer.lock().expect("PTY writer mutex poisoned");
        let writer = writer.as_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "action has exited")
        })?;
        writer.write_all(bytes)?;
        writer.flush()
    }

    pub fn interrupt(&self) -> std::io::Result<()> {
        self.write_input(&[3])
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<(), String> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| error.to_string())
    }

    pub fn cancel(&self) -> std::io::Result<()> {
        self.cancellation_requested.store(true, Ordering::SeqCst);
        self.killer
            .lock()
            .expect("child killer mutex poisoned")
            .kill()
    }

    pub fn try_recv(&mut self) -> Result<RuntimeEvent, TryRecvError> {
        let event = self
            .events
            .as_ref()
            .expect("runtime event receiver is present while session is active")
            .try_recv()?;
        if matches!(event, RuntimeEvent::Exited(_)) {
            self.exited = true;
            self.writer
                .lock()
                .expect("PTY writer mutex poisoned")
                .take();
        }
        Ok(event)
    }

    pub fn has_exited(&self) -> bool {
        self.exited
    }
}

fn wait_until_stopped(pid: u32) -> std::io::Result<()> {
    loop {
        let mut status = 0;
        let waited = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WUNTRACED) };
        if waited == -1 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if waited == pid as libc::pid_t && libc::WIFSTOPPED(status) {
            return Ok(());
        }
        return Err(std::io::Error::other(format!(
            "child exited before its PTY reader was ready (wait status {status})"
        )));
    }
}

fn copy_output(
    mut reader: Box<dyn Read + Send>,
    fd: std::os::fd::RawFd,
    child_exited: Arc<AtomicBool>,
    sender: SyncSender<RuntimeEvent>,
) {
    let mut buffer = vec![0; OUTPUT_CHUNK];
    loop {
        let mut descriptor = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe {
            libc::poll(
                &mut descriptor,
                1,
                OUTPUT_POLL_INTERVAL.as_millis() as libc::c_int,
            )
        };
        if ready == 0 {
            if child_exited.load(Ordering::Acquire) {
                break;
            }
            continue;
        }
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            break;
        }
        if descriptor.revents & (libc::POLLERR | libc::POLLNVAL) != 0 {
            break;
        }
        // FreeBSD may report trailing PTY bytes with POLLHUP but without
        // POLLIN. A read after hangup returns the buffered bytes, then EOF.
        if descriptor.revents & (libc::POLLIN | libc::POLLHUP) == 0 {
            continue;
        }
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                if sender
                    .send(RuntimeEvent::Output(buffer[..read].to_vec()))
                    .is_err()
                {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

impl Drop for RunningSession {
    fn drop(&mut self) {
        if !self.exited {
            self.cancellation_requested.store(true, Ordering::SeqCst);
            let _ = self
                .killer
                .lock()
                .expect("child killer mutex poisoned")
                .kill();
        }
        self.writer
            .lock()
            .expect("PTY writer mutex poisoned")
            .take();
        self.events.take();
        if let Some(monitor) = self.monitor.take() {
            let _ = monitor.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::TryRecvError;
    use std::sync::{Mutex, MutexGuard};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::catalog::load_embedded;
    use crate::host::Host;
    use crate::runtime::resolve;

    static PTY_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn serial_pty_test() -> MutexGuard<'static, ()> {
        PTY_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn start_fixture(name: &str) -> RunningSession {
        let bundle = load_embedded().unwrap();
        let action = bundle
            .catalog
            .actions
            .iter()
            .find(|action| action.id.as_str() == name)
            .unwrap();
        let command = resolve(&bundle.catalog, &bundle.files, &action.id, &Host::detect()).unwrap();
        RunningSession::start(command, 24, 80).unwrap()
    }

    fn wait_for_result(session: &mut RunningSession) -> (ExecutionResult, Vec<u8>) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut output = Vec::new();
        let result = loop {
            match session.try_recv() {
                Ok(RuntimeEvent::Output(bytes)) => output.extend(bytes),
                Ok(RuntimeEvent::Exited(result)) => break result,
                Ok(_) => {}
                Err(TryRecvError::Empty) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                event => panic!("fixture did not exit: {event:?}"),
            }
        };
        (result, output)
    }

    #[test]
    fn fast_exit_output_is_never_lost() {
        let _guard = serial_pty_test();
        for attempt in 1..=32 {
            let mut session = start_fixture("fixture.hello");
            let (result, output) = wait_for_result(&mut session);
            assert_eq!(
                result.status,
                ExecutionStatus::Succeeded,
                "attempt={attempt}, result={result:?}, output={}",
                String::from_utf8_lossy(&output)
            );
            assert!(
                String::from_utf8_lossy(&output).contains("Hello from Crank!"),
                "attempt {attempt} lost output"
            );
        }
    }

    #[test]
    fn forwards_input_to_an_interactive_fixture() {
        let _guard = serial_pty_test();
        let mut session = start_fixture("fixture.prompt");
        session.write_input(b"Ada\r").unwrap();
        let (result, output) = wait_for_result(&mut session);
        assert_eq!(result.status, ExecutionStatus::Succeeded);
        assert!(
            String::from_utf8_lossy(&output).contains("Hello, Ada!"),
            "interactive output was {}",
            String::from_utf8_lossy(&output)
        );
    }

    #[test]
    fn cancellation_has_a_distinct_result() {
        let _guard = serial_pty_test();
        let mut session = start_fixture("fixture.interrupt");
        session.cancel().unwrap();
        let (result, _) = wait_for_result(&mut session);
        assert_eq!(result.status, ExecutionStatus::Cancelled);
    }
}
