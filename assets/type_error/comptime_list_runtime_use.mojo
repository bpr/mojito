# expect: cannot materialize comptime value of type 'Array[Int, Int(3)]'
# A compile-time list is a fixed-size `Array` at runtime, which is not
# implicitly copyable either: it iterates under `comptime for` or crosses
# through `materialize[l]()`.
def main():
    comptime l = [1, 2, 3]
    print(len(l))
