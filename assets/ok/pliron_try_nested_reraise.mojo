# Nested try: an inner handler observes, wraps, and re-raises; the outer
# handler catches the wrapped error; a second try discards its binder.
def inner(n: Int) raises -> Int:
    if n > 2:
        raise Error("too big")
    return n * 10

def middle(n: Int) raises -> Int:
    try:
        return inner(n)
    except e:
        print("middle saw:", e)
        raise Error("wrapped")

def main():
    try:
        print(middle(1))
        print(middle(5))
    except e:
        print("main caught:", e)
    try:
        _ = inner(9)
    except:
        print("no binder path")
    print("end")
