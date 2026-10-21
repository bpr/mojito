# expect: against enum case of type 'Rec'
# A qualified enum case at the root names the subject's own type family.
struct Rec:
    var a: Int


def main():
    var o = Optional[Int](3)
    match o:
        case Rec.Some(x):
            print(x)
        case _:
            pass
