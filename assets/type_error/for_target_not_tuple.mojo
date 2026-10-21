# expect: type mismatch for tuple unpacking: expected a tuple, found Int
# A tuple loop target needs each element to be a tuple; a `List[Int]` yields `Int`.
def main():
    var xs: List[Int] = [1, 2]
    for a, b in xs:
        print(a, b)
