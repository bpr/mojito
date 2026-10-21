# An `if` whose arms are bare and whose result nothing reads is erased
# whole, its `elif` condition's call included, as the pin's
# `HoistYieldResults` erases the op with its condition regions: `f` never
# runs.
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
