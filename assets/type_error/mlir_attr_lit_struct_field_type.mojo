# A `#lit.struct` attribute's field value is parsed at the field's type, so
# `true` cannot set `SIMDLength`'s `index` field.
# expect: field '_mlir_value' of 'SIMDLength' in a '#lit.struct' attribute


def main():
    comptime n = __mlir_attr[`#lit.struct<{_mlir_value = true}> : `, SIMDLength]
    print(Int(n))
