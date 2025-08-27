SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"
echo "SCRIPT_DIR: $SCRIPT_DIR"
cd test_fuzzers
make all