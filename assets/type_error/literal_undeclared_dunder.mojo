# expect: has no method '__abs__'
# `IntLiteral` declares no `__abs__`, so a literal receiver's call is not
# resolved as `Int`'s.
def main():
    print((2).__abs__())
