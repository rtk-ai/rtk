# Homebrew Formula CI Automation

## Overview

The release workflow automatically updates the Homebrew formula in the [rtk-ai/homebrew-tap](https://github.com/rtk-ai/homebrew-tap) repository whenever a new version is released. This ensures the formula stays in sync with releases without manual intervention.

## How It Works

### Workflow Trigger
The `homebrew` job runs after the `release` job completes successfully on:
- Release publications (`release` event)
- Workflow calls from release-please
- Manual workflow dispatches

### Automation Steps

1. **Checkout Repositories**
   - Fetches the main rtk repository code to access the update script
   - Checks out the rtk-ai/homebrew-tap repository where the formula lives
   - Uses `HOMEBREW_TAP_TOKEN` secret for authentication

2. **Install GitHub CLI**
   - Installs `gh` CLI tool for interacting with GitHub API
   - Used to fetch release information and create PRs

3. **Get Version**
   - Extracts the release version from the trigger event
   - Supports all trigger types (release, workflow_call, workflow_dispatch)

4. **Wait for Release Assets**
   - Polls for `checksums.txt` availability (up to 5 minutes)
   - Ensures all release assets are uploaded before proceeding
   - Critical for SHA256 checksum retrieval

5. **Update Formula**
   - Runs `scripts/update-brew-formula.sh` in the homebrew-tap directory
   - Fetches latest checksums from GitHub release
   - Regenerates `Formula/rtk.rb` with:
     - New version number
     - Updated download URLs (pointing to rtk-ai/rtk)
     - Fresh SHA256 checksums for all platforms

6. **Commit Changes**
   - Creates a new branch in homebrew-tap: `homebrew-formula-update-{version}`
   - Commits the updated formula
   - Pushes to rtk-ai/homebrew-tap

7. **Create Pull Request**
   - Opens a PR in rtk-ai/homebrew-tap with the formula update
   - Includes detailed description of changes
   - Auto-labels as automated update

## Repository Setup

### Required Secret

The workflow requires a GitHub Personal Access Token with write access to rtk-ai/homebrew-tap:

1. Create a PAT with `repo` scope at https://github.com/settings/tokens
2. Add it as a secret named `HOMEBREW_TAP_TOKEN` in the rtk-ai/rtk repository
3. Ensure the token owner has write access to rtk-ai/homebrew-tap

### Workflow Configuration

The workflow checks out two repositories:
```yaml
- name: Checkout main repo
  uses: actions/checkout@v4
  with:
    path: rtk

- name: Checkout homebrew-tap repo
  uses: actions/checkout@v4
  with:
    repository: rtk-ai/homebrew-tap
    token: ${{ secrets.HOMEBREW_TAP_TOKEN }}
    path: homebrew-tap
```

## Manual Testing

To test the formula update script locally:

```bash
# Clone the homebrew-tap repository
git clone https://github.com/rtk-ai/homebrew-tap.git
cd homebrew-tap

# Set a GitHub token
export GITHUB_TOKEN=your_token

# Run the update script from the rtk repository
../rtk/scripts/update-brew-formula.sh

# Verify the formula
brew style Formula/rtk.rb
git diff Formula/rtk.rb
```

## Troubleshooting

### Formula Update Fails

**Problem**: Script can't fetch checksums
**Solution**: Ensure the rtk-ai/rtk release has a `checksums.txt` asset

**Problem**: Invalid SHA256 checksums
**Solution**: Verify release assets match expected naming conventions

**Problem**: gh CLI authentication fails
**Solution**: Check `GITHUB_TOKEN` permissions

### PR Creation Fails

**Problem**: Authentication error when pushing to homebrew-tap
**Solution**: Verify `HOMEBREW_TAP_TOKEN` secret is configured with write access

**Problem**: Branch already exists
**Solution**: The workflow handles this gracefully with `|| echo "PR may already exist"`

## Workflow Configuration

### Required Permissions
```yaml
permissions:
  contents: write  # For reading release assets
```

### Required Secrets
- `HOMEBREW_TAP_TOKEN`: Personal Access Token with write access to rtk-ai/homebrew-tap

### Environment Variables
- `GITHUB_TOKEN`: Automatically provided by GitHub Actions (for reading releases)

### Customization

To modify the automation:
- Edit `.github/workflows/release.yml` (homebrew job starting around line 206)
- Update `scripts/update-brew-formula.sh` for formula generation logic
- Adjust retry logic in "Wait for release assets" step

## Security Notes

- The workflow uses two tokens:
  - `GITHUB_TOKEN`: Automatically provided, read-only for release assets
  - `HOMEBREW_TAP_TOKEN`: User-provided PAT with write access to homebrew-tap
- All operations are scoped to specific repositories
- Formula updates require PR review in homebrew-tap before merge

## Future Improvements

Potential enhancements:
- Auto-merge PRs if all checks pass
- Add brew installation test in CI
- Support for homebrew-core submission
- Notification on failure
