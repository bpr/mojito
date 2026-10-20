# expect: cannot match value of type 'Point' against pattern type 'Int'
# A struct pattern's head must name the subject's own type.
@fieldwise_init
struct Point(ImplicitlyCopyable):
    var x: Int
    var y: Int


def main():
    match Point(1, 2):
        case Int(x=1):
            print("int")
