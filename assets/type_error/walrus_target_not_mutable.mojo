# expect: expression must be mutable in assignment
# A walrus takes any expression as its target, as an assignment does; one
# that is not an lvalue is rejected at the check.
def main():
    print(1 := 2)
