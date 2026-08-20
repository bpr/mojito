# A handled raise: the bound error prints and is destroyed; `else` is
# skipped on the error path; execution continues after the `try`.
def risky(n: Int) raises -> Int:
    var s: String = "local buffer"
    if n < 0:
        raise Error("boom")
    return n + 1

def main():
    try:
        print(risky(1))
        print(risky(-1))
        print("unreached")
    except e:
        print("caught:", e)
    else:
        print("no error")
    print("done")
