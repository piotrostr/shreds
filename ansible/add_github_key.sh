#!/bin/bash

if [ "$#" -ne 1 ]; then
    echo "Usage: $0 <ssh-public-key>"
    exit 1
fi

SSH_KEY="$1"

response=$(gh api \
  --method POST \
  -H "Accept: application/vnd.github+json" \
  -H "X-GitHub-Api-Version: 2022-11-28" \
  /user/keys \
  -f "title=Ansible-generated SSH Key" \
  -f "key=$SSH_KEY")

if [ $? -eq 0 ]; then
    echo "SSH key successfully added to GitHub"
else
    echo "Failed to add SSH key to GitHub"
    echo "Response: $response"
    exit 1
fi

