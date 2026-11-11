# expect: does not conform to trait 'Writable'
# `print(None)` passes the `None` type, which is not `Writable` at the pin.
def main():
    print(None)
