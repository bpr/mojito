# expect: operator '==' is not defined for None and None
# The `None` type has no `__eq__`, as at the pin.
def main():
    var x = None
    print(x == None)
