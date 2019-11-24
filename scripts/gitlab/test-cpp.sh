#!/bin/bash
echo "________Running test-cpp.sh________"
set -e # fail on any error
set -u # treat unset variables as error
set -B # enable brace expansion
#use nproc `linux only
THREADS=$(nproc)
export CC="sccache gcc"
ls /usr/bin{,local/}/clang*
export CXX="sccache clang++-8"

echo "________Running the C++ example________"
DIR=parity-clib/examples/cpp/build
mkdir -p $DIR
cd $DIR
cmake ..
make VERBOSE=1 -j $THREADS
# Note: we don't try to run the example because it tries to sync Kovan, and we don't want
#       that to happen on CI
cd -
rm -rf $DIR
#show sccache statistics
sccache --show-stats
