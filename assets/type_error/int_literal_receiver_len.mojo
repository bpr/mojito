# A literal receiver is no `SIMD` instance: `SIMD`'s declared `__len__`
# is not its attribute, as the pin reports for `IntLiteral[5]`.
# expect: has no method '__len__'
def main():
    print((5).__len__())
