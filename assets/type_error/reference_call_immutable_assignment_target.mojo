# expect: immutable reference returned by call 'peek()'
# A call returning an immutable reference is no assignment target.
def peek(s: String) -> ref[origin_of(s)] String:
    return s


def main():
    var s = String("a")
    peek(s) = String("b")
    print(s)
