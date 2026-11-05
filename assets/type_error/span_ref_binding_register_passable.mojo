# expect: doesn't have a memory origin in 'ref' binding
# A `Span` is `TrivialRegisterPassable`, so a `ref` binding cannot name one
# that is an rvalue: `ref r = values[1:]` and the starred `case [f, *ref rr]`
# are both rejected.
def main():
    var values: List[Int] = [1, 2, 3]
    ref r = values[1:]
    print(len(r))
    match values:
        case [f, *ref rr]:
            print(f, len(rr))
        case _:
            pass
