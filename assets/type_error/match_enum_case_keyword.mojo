# expect: do not support keyword arguments
# An enum case's payload pattern is positional.
def main():
    var o = Optional[Int](3)
    match o:
        case .Some(v=x):
            print(x)
        case _:
            pass
