#!/bin/sh
line=1
while [ "$line" -le 2000 ]; do
    printf 'output line %s\n' "$line"
    line=$((line + 1))
done
