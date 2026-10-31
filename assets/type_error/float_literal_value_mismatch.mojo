# expect: cannot implicitly convert 'FloatLiteral[0.20000000000000001]' value to 'FloatLiteral[0.10000000000000001]'
# A float literal's type carries its value, spelled as the pin spells a
# `#pop.float_literal` attribute (`%.17g` of the nearest binary64).
def main():
    var y: type_of(0.1) = 0.2
    print(y)
