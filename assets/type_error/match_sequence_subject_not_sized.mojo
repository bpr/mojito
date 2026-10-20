# expect: value of type 'String' does not conform to 'Sized'
# A sequence pattern's subject must conform to `Sized`; `String` does not.
def main():
    var s = String("hi")
    match s:
        case [a, b]:
            print(a, b)
        case _:
            print("other")
