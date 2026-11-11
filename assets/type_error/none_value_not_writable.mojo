# expect: does not conform to trait 'Writable'
# The `None` type is not `Writable` at the pin; only the `NoneType` struct
# is.
def main():
    var x = None
    print(x)
