# `DType.<name>` is an ordinary runtime value: it binds, prints its name, and
# is not confined to SIMD brackets and `[dtype: DType]` arguments.
def main():
    var x = DType.float32
    print(x)
