# expect: value of type 'List[Int]' cannot be implicitly copied
# Unpacking a tuple place copies each element into its target, so an element
# that is not `ImplicitlyCopyable` cannot be unpacked into a name.
def main():
    var pair: Tuple[Int, List[Int]] = (3, [1, 2])
    var a, b = pair
    print(a, len(b))
