# A vector of addresses `!kgen.simd<4, address>` is a type of its own, so an
# integer literal does not initialize one.
# expect: type mismatch for variable 'x': expected __mlir_type.`!kgen.simd<4, address>`, found IntLiteral[1]
def main():
    var x: __mlir_type.`!kgen.simd<4, address>` = 1
