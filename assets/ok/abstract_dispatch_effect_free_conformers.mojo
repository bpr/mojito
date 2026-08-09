trait Speaker:
    def speak(self): ...

struct Dog(Speaker):
    def __init__(out self):
        pass

    def speak(self):
        print("Woof!")

def hail[T: Speaker](imm s: T):
    s.speak()

def hail(x: Int):
    print(x)

def main():
    var d = Dog()
    hail(d)
    hail(3)
