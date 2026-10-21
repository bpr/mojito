# expect: must be mutable
# A bare payload binding is an immutable reference to the payload.
def main():
    var o = Optional[Int](3)
    match o:
        case .Some(x):
            x += 1
        case _:
            pass
