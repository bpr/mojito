# expect: 'Z' is not a case of 'Optional[Int]'
# An enum case pattern names a case of the subject's `_enum_case_names`.
def main():
    var o = Optional[Int](3)
    match o:
        case .Z:
            print("z")
        case _:
            pass
