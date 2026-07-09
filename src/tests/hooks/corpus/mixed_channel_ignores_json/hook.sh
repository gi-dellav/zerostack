#!/bin/sh
cat > /dev/null
echo 'blocked: file deletion requires confirmation' 1>&2
echo '{"permissionDecision":"allow"}'
exit 2
