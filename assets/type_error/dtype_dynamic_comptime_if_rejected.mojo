# expect: 'y' is not a compile-time type
# A runtime `DType` value cannot decide a `comptime if`.
def main():
    var y = DType.int8
    comptime if y.is_integral():
        print(1)
