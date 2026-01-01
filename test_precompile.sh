#!/bin/bash
set -e

echo "======================================"
echo "Testing Native PRAF Precompile @ 0x805"
echo "======================================"
echo ""

RPC="http://localhost:9545"
PRECOMPILE_ADDR="0x0000000000000000000000000000000000000805"

test_count=0
pass_count=0

# Helper function to test RPC call
test_call() {
    local name=$1
    local selector=$2
    local expected=$3
    
    echo "TEST $((++test_count)): $name"
    result=$(curl -s -X POST $RPC -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"method\":\"eth_call\",\"params\":[{\"to\":\"$PRECOMPILE_ADDR\",\"data\":\"$selector\"},\"latest\"],\"id\":1}" \
        | grep -o '"result":"[^"]*"' | cut -d'"' -f4)
    
    echo "  Selector: $selector"
    echo "  Result:   $result"
    echo "  Expected: $expected"
    
    if echo "$result" | grep -q "$expected"; then
        echo "  ✅ PASS"
        ((pass_count++))
    else
        echo "  ❌ FAIL"
        return 1
    fi
    echo ""
}

# Run tests
echo "Running precompile function tests..."
echo ""

# Test 1: symbol() - should return "PRAF"
test_call "symbol()" "0x95d89b41" "50524146"

# Test 2: name() - should return "PRAPH"  
test_call "name()" "0x06fdde03" "5052415048"

# Test 3: decimals() - should return 18 (0x12)
test_call "decimals()" "0x313ce567" "0x12"

# Test 4: Unknown selector - should return empty
test_call "unknown selector" "0xffffffff" "0x"

echo "======================================"
echo "Test Results: $pass_count/$test_count passed"
echo "======================================"

if [ $pass_count -eq $test_count ]; then
    echo "✅ All precompile tests PASSED!"
    exit 0
else
    echo "❌ Some tests FAILED!"
    exit 1
fi
