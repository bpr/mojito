# expect: '@always_inline("builtin")' only supports calls to other '@always_inline("builtin")' functions
# Comparing a `Float64` with a float literal converts the literal through a
# non-builtin initializer.
@always_inline("builtin")
def is_one(n: Float64) -> Bool:
    return n == 1.0


def main():
    print(is_one(3))
