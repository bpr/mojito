# expect: cannot materialize comptime value of type 'Array[Int, Int(3)]'
# A function body's subscript of a module list constant materializes the
# whole `Array`, which is not implicitly copyable, though the constant is
# evaluated on demand. A compile-time binding of the element reads it.
def twice(n: Int) -> Int:
    return n * 2

comptime XS = [1, twice(3), 5]

def main():
    print(XS[1])
