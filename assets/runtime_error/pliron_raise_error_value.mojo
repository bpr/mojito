# expect: unhandled error: boom from a var
def main() raises:
    var msg = "boom from a var"
    print("before")
    raise Error(msg)
