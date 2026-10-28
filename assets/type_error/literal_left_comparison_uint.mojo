# A comparison has no reflected dunder, so a literal left operand
# materializes to `Int`, whose `__lt__` takes no `UInt`, as at the pin.
# expect: operator '<' is not defined for Int and UInt
def main():
    print(3 < UInt(4))
