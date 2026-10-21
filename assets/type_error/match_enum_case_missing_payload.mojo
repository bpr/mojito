# expect: requires a payload pattern inside the parentheses
# Empty parentheses after a payload case are rejected; write `.Some(_)`
# or the bare tag `.Some`.
def main():
    var o = Optional[Int](3)
    match o:
        case .Some():
            print("some")
        case _:
            pass
