# Probe from R1397: `Int(mlir_value=...)` takes the `!kgen.scalar<index>`
# storage of an `Int`, not an `Int`.
# expect: type mismatch for argument 'mlir_value' of 'Int': expected __mlir_type.`!kgen.scalar<index>`, found Int
def main():
    var n = 4
    print(Int(mlir_value=n))
