use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use thiserror::Error;

use crate::catalog::ActionId;
use crate::runtime::ResolvedCommand;

const EVENT_BUFFER: usize = 128;
const OUTPUT_CHUNK: usize = 8 * 1024;

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
        let mut builder = CommandBuilder::new(&command.program);
        builder.args(&command.args);
        builder.cwd(&command.working_directory);
        let mut child = pair
            .slave
            .spawn_command(builder)
            .map_err(|error| StartError::Spawn {
                action: action_id.clone(),
                message: error.to_string(),
            })?;
        // FreeBSD can report EOF when the master is read before a child has
        // attached to the slave. Start the reader only after spawning, then
        // wait until its thread is running before closing our slave handle.
        let (reader_ready_sender, reader_ready) = sync_channel(0);
        let output_sender = sender.clone();
        let output = thread::Builder::new()
            .name("crank-action-output".to_owned())
            .spawn(move || {
                let _ = reader_ready_sender.send(());
                copy_output(reader, output_sender);
            })
            .map_err(StartError::Monitor)?;
        let _ = reader_ready.recv();
        drop(pair.slave);
        let killer = child.clone_killer();
        let cancellation_requested = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::clone(&cancellation_requested);
        let monitor = thread::spawn(move || {
            let status = child.wait();
            let _ = output.join();
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

fn copy_output(mut reader: Box<dyn Read + Send>, sender: SyncSender<RuntimeEvent>) {
    let mut buffer = vec![0; OUTPUT_CHUNK];
    loop {
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
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::catalog::load_embedded;
    use crate::host::Host;
    use crate::runtime::resolve;

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
        let mut session = start_fixture("fixture.prompt");
        session.write_input(b"Ada\r").unwrap();
        let (result, output) = wait_for_result(&mut session);
        assert_eq!(result.status, ExecutionStatus::Succeeded);
        assert!(String::from_utf8_lossy(&output).contains("Hello, Ada!"));
    }

    #[test]
    fn cancellation_has_a_distinct_result() {
        let mut session = start_fixture("fixture.interrupt");
        session.cancel().unwrap();
        let (result, _) = wait_for_result(&mut session);
        assert_eq!(result.status, ExecutionStatus::Cancelled);
    }
}
