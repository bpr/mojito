# expect: 'Z' is not a case of 'Optional[Int]'
# A case pattern names one of the subject's declared case names.
def main():
    var o = Optional[Int](3)
    match o:
        case .Z:
            print("z")
        case _:
            pass
