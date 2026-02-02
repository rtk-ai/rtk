# Homebrew Formula CI Automation

## Overview

The release workflow automatically updates the Homebrew formula whenever a new version is released. This ensures the formula stays in sync with releases without manual intervention.

## How It Works

### Workflow Trigger
The `homebrew` job runs after the `release` job completes successfully on:
- Release publications (`release` event)
- Workflow calls from release-please
- Manual workflow dispatches

### Automation Steps

1. **Checkout Repository**
   - Fetches the repository code with full history
   - Required for creating branches and commits

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
   - Runs `scripts/update-brew-formula.sh`
   - Fetches latest checksums from GitHub release
   - Regenerates `Formula/rtk.rb` with:
     - New version number
     - Updated download URLs
     - Fresh SHA256 checksums for all platforms

6. **Commit Changes**
   - Creates a new branch: `homebrew-formula-update-{version}`
   - Commits the updated formula
   - Pushes to origin

7. **Create Pull Request**
   - Opens a PR with the formula update
   - Includes detailed description of changes
   - Auto-labels as automated update

## Manual Testing

To test the automation locally:

```bash
# Set a GitHub token
export GITHUB_TOKEN=your_token

# Run the update script
./scripts/update-brew-formula.sh

# Verify the formula
brew style Formula/rtk.rb
git diff Formula/rtk.rb
```

## Troubleshooting

### Formula Update Fails

**Problem**: Script can't fetch checksums
**Solution**: Ensure the release has a `checksums.txt` asset

**Problem**: Invalid SHA256 checksums
**Solution**: Verify release assets match expected naming conventions

**Problem**: gh CLI authentication fails
**Solution**: Check `GITHUB_TOKEN` permissions (needs `contents: write`)

### PR Creation Fails

**Problem**: Branch already exists
**Solution**: The workflow handles this gracefully with `|| echo "PR may already exist"`

**Problem**: PR already exists
**Solution**: The `gh pr create` command will fail gracefully

## Workflow Configuration

### Required Permissions
```yaml
permissions:
  contents: write  # For pushing branches and creating PRs
```

### Environment Variables
- `GITHUB_TOKEN`: Automatically provided by GitHub Actions

### Customization

To modify the automation:
- Edit `.github/workflows/release.yml` (line 206+)
- Update `scripts/update-brew-formula.sh` for formula generation logic
- Adjust retry logic in "Wait for release assets" step

## Security Notes

- The workflow uses `GITHUB_TOKEN` (not a personal access token)
- No secrets or credentials are required
- All operations are scoped to the repository
- Formula updates require PR review before merge

## Future Improvements

Potential enhancements:
- Auto-merge PRs if all checks pass
- Add brew installation test in CI
- Support for homebrew-core submission
- Notification on failure
