# expect: invalid call to 'reverse'
# `Tuple.reverse` declares no compile-time parameter of its own.
def main():
    var pair = Tuple(1, True)
    print(pair.reverse[Int]()[0])
