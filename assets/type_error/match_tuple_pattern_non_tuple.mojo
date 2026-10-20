# expect: expected a tuple type to match against
# A tuple pattern needs a tuple subject.
def main():
    var n = 1
    match n:
        case (a, b):
            print(a)
