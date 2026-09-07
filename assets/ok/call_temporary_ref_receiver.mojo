# A `ref self` method on a call temporary (`s.split(",")[1]`): the temporary
# lives through the subscript, so the result binds without an intermediate.
def main():
    var source = String("left,right")
    print(source.split(",")[1])
    print(String("a b c").split(" ")[0], len(source.split(",")))
    var parts = source.split(",")
    print(parts[0])
