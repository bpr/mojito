# expect: enum case 'None' has no associated value
# A case whose payload type is `NoneType` takes no payload pattern.
def main():
    var o = Optional[Int](3)
    match o:
        case .None(x):
            print(x)
        case _:
            pass
