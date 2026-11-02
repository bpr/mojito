# An `if` whose arms are bare and whose result nothing reads still runs a
# call in its `elif` condition: `HoistYieldResults` erases such an `if` only
# when no condition region computes anything, and `f` runs, as at the pin
# since `1.3.0.dev2026110105`.
@no_inline
def f() -> Bool:
    print("f ran")
    return True


def main():
    var c = False
    if c:
        pass
    elif f():
        pass
    print("done")
