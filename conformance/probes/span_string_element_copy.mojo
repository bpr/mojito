# A `String` element read out of a `Span` as a whole value (`String(span[0])`,
# `span[0].copy()`) is refused: here with "access to 'items' conflicts with
# live reference 'span'", and alone with the VM trap "invalid reference
# projection Field("_data") on None". The pin prints "x x". An `Int` span,
# the list's own element, and `span[0] + "y"` all run.
def main():
    var items = List[String]()
    items.append("x")
    var span: Span[String, _] = items
    var copied = span[0].copy()
    print(String(span[0]), copied)
