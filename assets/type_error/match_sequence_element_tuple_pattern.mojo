# expect: expected a tuple type to match against
# A sequence element is untyped, so a tuple pattern cannot match it.
def main():
    var xs: List[Tuple[Int, Int]] = [(1, 2)]
    match xs:
        case [(u, v)]:
            print(u, v)
        case _:
            print("other")
