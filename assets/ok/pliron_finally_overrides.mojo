# A `finally` outcome wins: over a pending return, over a pending raise, and
# a raise that survives an untouched `finally` still reaches the handler.
def f(x: Int) -> Int:
    try:
        return 1
    finally:
        print("fin")
        return 2

def g() raises -> Int:
    try:
        raise Error("pending")
    finally:
        print("g fin")
        return 3

def h(x: Int) raises -> Int:
    try:
        if x > 0:
            raise Error("herr")
        return 4
    finally:
        print("h fin", x)

def main():
    print(f(0))
    try:
        print(g())
    except e:
        print("unreached", e)
    try:
        print(h(1))
    except e:
        print("caught", e)
    try:
        print(h(-1))
    except e:
        print("unreached 2", e)
