# Probe from R1397: `Int(mlir_value=...)` takes the `!kgen.scalar<index>`
# storage of an `Int`, not an `Int`.
# expect: no constructor overload matches the supplied arguments
def main():
    var n = 4
    print(Int(mlir_value=n))
