# expect: expression must be mutable in assignment
# Any expression parses as an assignment target; one that is not an lvalue
# is rejected at the check.
def main():
    var a = 1
    a + 1 = 2
    print(a)
