# Probe from R1397: an `Int`'s `!kgen.scalar<index>` storage is an MLIR
# type, which conforms to no trait, so `print` cannot write it.
# expect: does not conform to trait 'Writable'
def main():
    var s: __mlir_type.`!kgen.scalar<index>` = Int(4)._mlir_value
    print(s)
