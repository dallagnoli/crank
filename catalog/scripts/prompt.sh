#!/bin/sh
printf 'What is your name? '
IFS= read -r name
printf 'Hello, %s!\n' "$name"

