# expect: '@always_inline' operand must be "nodebug" or "builtin"
@always_inline("sometimes")
def twice(x: Int) -> Int:
    return x + x

def main():
    print(twice(2))
