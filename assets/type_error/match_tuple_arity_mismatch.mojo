# expect: of 2 elements against a pattern with 3 elements
# A tuple pattern must have the subject's static arity.
def main():
    match (1, 2):
        case (a, b, c):
            print(a)
