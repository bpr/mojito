# `None` is no literal pattern: compare against it with `== None`, or match
# an `Optional`'s case with `.None`.
# expect: expression is not a valid match pattern; use '==' to compare against its value
def main():
    var o: Optional[Int] = None
    match o:
        case None:
            print("none")
        case _:
            print("other")
