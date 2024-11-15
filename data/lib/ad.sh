#!/usr/bin/env bash
# Helper functions for writing scripts to interact with ad

[ -e "$HOME/.profile" ] && source ~/.profile

# Write a control message to ad.
# The format accepted is the same as when using the internal command line
adCtl() { echo -n "$*" | 9p write ad/ctl; }

# Execute an Edit script within the current buffer
adEdit() { adCtl "Edit $*"; }

# Read the contents of the index file
adIndex() { 9p read ad/buffers/index; }

# Display an error in the editor status line and exit
adError() {
  adCtl "echo $*"
  exit 1
}

# Exit with an error message if this script was not launched from ad itself
requireAd() {
  [[ -z "$bufid" ]] && adError "need to be run from inside of ad"
}

# Read the contents of an fsys file for the specified buffer
bufRead() { 9p read "ad/buffers/$1/$2"; }

# Write a string to the specified buffer file
bufWrite() { 9p write "ad/buffers/$1/$2"; }

# Follow the ad log stream of ongoing buffer events
adLog() { 9p read ad/log; }

# Fetch the id of the currently focused buffer
currentBufferId() { 9p read ad/buffers/current; }

# Set focus to the buffer with the specified id
focusBuffer() { echo "$1" | 9p write ad/buffers/current; }

# Clear the contents of the current buffer
clearBuffer() {
  echo -n "," | bufWrite "$1" xaddr
  echo -n "" | bufWrite "$1" xdot
}

# Mark the buffer with the specified id as clean
markClean() { adCtl "mark-clean $1"; }

# Set the cursor position for the specified buffer to the begining of the file
curToBof() { echo -n 0 | bufWrite "$1" addr; }

# Set the cursor position for the specified buffer to the end of the file
curToEof() { echo -n '$' | bufWrite "$1" addr; }

# dmenu style selection from newline delimited input on stdin
minibufferSelect() {
  9p write ad/minibuffer
  [ -n "$1" ] && adCtl "minibuffer-prompt $1"
  9p read ad/minibuffer
}
