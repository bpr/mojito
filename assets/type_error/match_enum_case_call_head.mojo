# expect: must be written as 'Type.Case(...)'
# Over an `EnumLike` subject every call-shaped pattern is an enum case, so
# its head must name a case.
def main():
    var o = Optional[Int](3)
    match o:
        case Optional(x):
            print(x)
        case _:
            pass
