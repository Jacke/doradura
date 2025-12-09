#!/bin/bash
# Script to replace admin command handlers in main.rs

# Backup original file
cp src/main.rs src/main.rs.backup

# Create temp file with the replacement for Command::Users
cat > /tmp/users_replacement.txt << 'EOF'
                                Command::Users => {
                                    let username = msg.from.as_ref().and_then(|u| u.username.as_deref());
                                    let _ = handle_users_command(&bot, msg.chat.id, username, db_pool.clone()).await;
                                }
EOF

# Create temp file with the replacement for Command::Setplan
cat > /tmp/setplan_replacement.txt << 'EOF'
                                Command::Setplan => {
                                    let username = msg.from.as_ref().and_then(|u| u.username.as_deref());
                                    let message_text = msg.text().unwrap_or("");
                                    let _ = handle_setplan_command(&bot, msg.chat.id, username, message_text, db_pool.clone()).await;
                                }
EOF

# Create temp file with the replacement for Command::Admin
cat > /tmp/admin_replacement.txt << 'EOF'
                                Command::Admin => {
                                    let username = msg.from.as_ref().and_then(|u| u.username.as_deref());
                                    let _ = handle_admin_command(&bot, msg.chat.id, username, db_pool.clone()).await;
                                }
EOF

echo "Replacement files created"
echo "Now you need to manually edit main.rs to replace these command handlers"
echo "Backup saved as src/main.rs.backup"
