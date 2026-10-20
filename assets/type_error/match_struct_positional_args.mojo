# expect: struct patterns do not support positional
# A struct pattern names its fields; it takes no positional pattern.
@fieldwise_init
struct Point(ImplicitlyCopyable):
    var x: Int
    var y: Int


def main():
    match Point(1, 2):
        case Point(1, y):
            print(y)
