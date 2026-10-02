# A `mut` parameter is the caller's storage, so a body that transfers it away
# must write a value back before it returns.
# expect: 'x' is uninitialized at return from this function
# requires: stdlib
def take(mut x: String) -> String:
    return x^


def main():
    var s = String("a")
    print(take(s))
