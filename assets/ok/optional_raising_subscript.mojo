# The raising empty subscript `opt[]` (current Optional's `__getitem__`): a
# reference to the payload, or `EmptyOptionalError[T]` when the Optional is
# empty.
def first_or_report(opt: Optional[Int]) -> String:
    try:
        return String(opt[])
    except error:
        return String(error)

def main():
    var some = Optional[Int](7)
    try:
        print(some[])
        some[] += 1
        print(some[], some.value())
    except error:
        print("unexpected", error)
    var empty = Optional[Int]()
    print(first_or_report(some), first_or_report(empty))
    var text = Optional[String]("word")
    try:
        print(text[].byte_length(), text[])
    except error:
        print("unexpected", error)
    var missing = Optional[String]()
    try:
        print(missing[])
    except error:
        print(error)
