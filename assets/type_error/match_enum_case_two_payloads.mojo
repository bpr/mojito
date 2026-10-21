# expect: at most one payload subpattern
# An enum case takes one payload pattern; a tuple payload is `.Some((a, b))`.
def main():
    var o = Optional[Int](3)
    match o:
        case .Some(x, y):
            print(x, y)
        case _:
            pass
