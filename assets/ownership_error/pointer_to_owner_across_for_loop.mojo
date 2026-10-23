# expect: conflicts with live reference
# A `for` loop between a pointer's making and its owner's mutation leaves the
# pointer's loan live: the loop leaves through the handler of the `try`
# around its `__next__` call, which the loan check follows to the code after
# the loop.
def main():
    var xs = List[Int]()
    xs.append(1)
    var p = Pointer(to=xs)
    for i in range(2):
        print(i)
    xs.append(9)
    print(p[][0])
