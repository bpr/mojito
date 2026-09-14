# Unpacking a named Tuple copies each element: the pin prints `3 seven 3`,
# while Mojito's VM reports "use after Pointer deallocation" because the
# unpack shares the String buffer with `pair`. With a `List[Int]` element
# the pin rejects the unpack ("cannot be implicitly copied"); Mojito accepts
# it and frees the element twice.
def main():
    var pair: Tuple[Int, String] = (3, "seven")
    var first, second = pair
    var left: Int = pair[0]
    print(first, second, left)
