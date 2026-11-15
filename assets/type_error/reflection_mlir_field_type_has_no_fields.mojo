# expect: requires a struct type
# A scalar's reflected field type is `SIMD`'s `!kgen.scalar` storage, an
# MLIR type with no fields, so a field query over it is rejected, as at the
# pin.
def main():
    comptime T = reflect[Int].field["_mlir_value"].T
    print(reflect[T].field_count())
