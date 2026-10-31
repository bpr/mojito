# expect: cannot implicitly convert 'IntLiteral[7]' value to 'IntLiteral[4]'
# A literal's type carries its value, so a literal of another value is
# not one of its values.
comptime N = 4


def main():
    var y: type_of(N) = 7
    print(y)
