# expect: cannot materialize comptime value of type 'Array[Int, Int(2)]'
# A local `comptime` binding of a display over a generic body's parameters is
# a compile-time value with no runtime form: a runtime read of it would
# materialize the whole `Array`, which is not implicitly copyable. It is read
# in a compile-time position or crosses through `materialize[L]()`.
def second[n: Int]() -> Int:
    comptime L = [n, n * 2]
    return L[1]


def main():
    print(second[3]())
