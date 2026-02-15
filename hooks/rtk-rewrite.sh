#!/bin/bash
# Migration shim — existing installs forward to rtk binary.
# New installs use "rtk hook claude" directly. Remove in a future release.
exec rtk hook claude
