# expect: expected Optional[Tuple[Int8, Int8]], found Tuple[Int, Int]
# `(1, 2)` deduces `Tuple[Int, Int]` before `Optional`'s `@implicit`
# initializer sees it, so the payload type does not convert element-wise.
def main():
    var x: Optional[Tuple[Int8, Int8]] = (1, 2)
    print(x.value()[0])
