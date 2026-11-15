# expect: must be written as 'Type.Case(...)'
# Over a subject that declares cases every call-shaped pattern is a case
# pattern, so its head must name a case.
def main():
    var o = Optional[Int](3)
    match o:
        case Optional(x):
            print(x)
        case _:
            pass
