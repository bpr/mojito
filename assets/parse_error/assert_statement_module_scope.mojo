# expect: 'assert' must be inside a function
# Upstream allows an `assert` statement only in a function body.

assert 1 == 1, "module"


def main():
    print("ok")
