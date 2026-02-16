#!/bin/sh
# Legacy shim — actual hook logic is in the rtk binary.
# Direct usage: rtk hook claude (reads JSON from stdin)
exec rtk hook claude
