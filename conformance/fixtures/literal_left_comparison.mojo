# A comparison has no reflected dunder: a literal left operand materializes
# to `Int`, whose `__lt__` and `__eq__` take no `UInt`, so Mojito and the pin
# both reject `3 < UInt(4)` ("no matching method in call to '__lt__'").
def main():
    print(3 < UInt(4), 3 == UInt(3))
