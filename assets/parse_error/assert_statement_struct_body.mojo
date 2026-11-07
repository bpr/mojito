# expect: 'assert' must be inside a function
# A struct body holds members, not an `assert` statement, as at the pin.

struct S:
    assert True


def main():
    print("ok")
