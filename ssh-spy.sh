#!/bin/bash
echo "$(date -Iseconds) SSH called with args: $@" >> /c/Users/Rose/Documents/projects/braid/ssh-spy.log
echo "SSH_AUTH_SOCK=$SSH_AUTH_SOCK" >> /c/Users/Rose/Documents/projects/braid/ssh-spy.log
echo "---" >> /c/Users/Rose/Documents/projects/braid/ssh-spy.log
# Actually run ssh so claude doesn't break
exec ssh "$@"
