# The empty subscript is the pointer dereference; a non-pointer receiver
# requires an index argument.
# expect: is not a pointer
def main():
    var xs: List[Int] = [1, 2]
    print(xs[])
