# expect: cannot use a dynamic value in a parameter list
def main():
    var x = True
    print(materialize[x]())
