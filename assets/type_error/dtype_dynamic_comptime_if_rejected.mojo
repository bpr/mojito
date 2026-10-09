# expect: cannot use a dynamic value in 'comptime if' condition
# A runtime `DType` value cannot decide a `comptime if`.
def main():
    var y = DType.int8
    comptime if y.is_integral():
        print(1)
