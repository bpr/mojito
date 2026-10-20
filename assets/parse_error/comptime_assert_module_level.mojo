# expect: 'comptime assert' must be inside a function
# Upstream allows a `comptime assert` only in a function body.

comptime assert True, "module"


def main():
    print("ok")
