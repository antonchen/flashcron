#!/bin/sh
if [ -d /data ]; then
    chmod o+x /data || exit 1
    find /data -type d -exec chmod o+x {} \; || exit 1
fi
exec /usr/local/bin/flashcron "$@" run
