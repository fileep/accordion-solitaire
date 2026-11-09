#!/bin/bash
python anatomy.py 18238151902 7615968364 5895817790 > anatomy2.log 2>&1
python falsify.py > falsify.log 2>&1
echo done > chain.done
