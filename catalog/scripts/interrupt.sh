#!/bin/sh
trap 'printf "Interrupted.\n"; exit 130' INT
printf 'Waiting for Ctrl-C...\n'
while :; do
    sleep 1
done
