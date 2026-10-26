# A `finally` outcome wins: over a pending return, over a pending raise
# (one out of the function's handler too, whose error dies in the
# function's error slot), and a raise that survives an untouched `finally`
# still reaches the handler.
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

def k() raises -> Int:
    try:
        raise Error("first")
    except:
        raise Error("second")
    finally:
        return 7

def main():
    print(f(0))
    try:
        print(k())
    except e:
        print("unreached 3", e)
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
