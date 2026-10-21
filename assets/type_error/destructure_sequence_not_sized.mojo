# expect: value of type 'String' does not conform to 'Sized'
# A sequence destructuring's value must conform to `Sized`; `String` does not.
def main() raises:
    var s = String("hi")
    var [a, b] = s
    print(a, b)
