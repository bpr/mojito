# expect: expression must be mutable in assignment
# A `ref` leaf of a sequence target binds its element to read.
def main() raises:
    var values: List[Int] = [1, 2]
    [ref a, var b] = values
    print(a, b)
    a = 5
    print(values[0], b)
