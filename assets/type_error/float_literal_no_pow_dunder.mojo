# expect: has no method '__pow__'
# `FloatLiteral` declares no `__pow__`, so a literal receiver's call is not
# resolved as `Float64`'s.
def main():
    print((2.5).__pow__(2.0))
