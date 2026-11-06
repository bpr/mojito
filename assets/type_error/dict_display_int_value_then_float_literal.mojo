# expect: the SIMD type must be floating point
# A dictionary display binds `V` from its first value, `Int`, rather than
# merging its values, so `4.5` converts to `Int` and fails the assert.
def main():
    var d = {1: 2, 3: 4.5}
    print(len(d))
